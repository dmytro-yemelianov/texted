//! Ordered async buffer commands, snapshot publication and fenced bulk commits.
use crate::{commands::EditorCommand, editing::EditorModel, selection::SelectionState};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
};
use texted_buffer::BufferSnapshot;
pub const PENDING_LIMIT: usize = 64;
pub const BULK_THRESHOLD: usize = 64 * 1024;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditContext {
    pub document: u64,
    pub revision: u64,
    pub cursor: usize,
    pub selection: SelectionState,
}
#[derive(Clone)]
pub struct Published {
    pub snapshot: BufferSnapshot,
    pub selection: SelectionState,
    pub document: u64,
    pub completed_through: u64,
    pub busy: Option<u64>,
}
impl Published {
    pub fn context(&self) -> EditContext {
        EditContext {
            document: self.document,
            revision: self.snapshot.revision,
            cursor: self.snapshot.cursor_offset,
            selection: self.selection,
        }
    }
}
pub enum Command {
    Edit(EditorCommand),
    Load(ropey::Rope),
    BreakUndoGroup,
    Replace {
        query: String,
        replacement: String,
        all: bool,
    },
    DeleteContext(EditContext),
    PasteContext {
        context: EditContext,
        text: String,
    },
}
struct Envelope {
    sequence: u64,
    document: u64,
    command: Command,
}
#[derive(Debug)]
pub struct Completion {
    pub sequence: u64,
    pub result: Result<(), String>,
}
pub enum SubmitError {
    Busy(Command),
    Closed(Command),
}
struct Admission {
    next: u64,
    pending: usize,
    document: u64,
}
struct Shared {
    published: Mutex<Arc<Published>>,
    changed: Condvar,
    admission: Mutex<Admission>,
    stop: AtomicBool,
    wake: Arc<dyn Fn() + Send + Sync>,
    progress: AtomicUsize,
    #[cfg(test)]
    gate: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
#[derive(Clone)]
pub struct BufferClient {
    commands: Sender<Envelope>,
    cancel: Sender<()>,
    shared: Arc<Shared>,
    completions: Receiver<Completion>,
}
impl BufferClient {
    /// A successful return is acceptance. Busy/Closed return ownership to the caller.
    pub fn submit(&self, command: Command) -> Result<u64, SubmitError> {
        let mut admission = self
            .shared
            .admission
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if admission.pending >= PENDING_LIMIT {
            return Err(SubmitError::Busy(command));
        }
        let document = admission.document + u64::from(matches!(command, Command::Load(_)));
        let sequence = admission.next;
        match self.commands.try_send(Envelope {
            sequence,
            document,
            command,
        }) {
            Ok(()) => {
                admission.next += 1;
                admission.pending += 1;
                admission.document = document;
                Ok(sequence)
            }
            Err(crossbeam_channel::TrySendError::Full(e)) => Err(SubmitError::Busy(e.command)),
            Err(crossbeam_channel::TrySendError::Disconnected(e)) => {
                Err(SubmitError::Closed(e.command))
            }
        }
    }
    pub fn latest(&self) -> Arc<Published> {
        self.shared
            .published
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub fn barrier(&self) -> u64 {
        self.shared
            .admission
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .next
            - 1
    }
    pub fn drain(&self) -> Vec<Completion> {
        let result: Vec<_> = self.completions.try_iter().collect();
        let mut admission = self
            .shared
            .admission
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        admission.pending = admission.pending.saturating_sub(result.len());
        result
    }
    pub fn cancel(&self) {
        let _ = self.cancel.try_send(());
    }
    pub fn progress(&self) -> usize {
        self.shared.progress.load(Ordering::Relaxed)
    }
}
pub struct BufferService {
    pub client: BufferClient,
    shutdown: Sender<()>,
    thread: Option<thread::JoinHandle<()>>,
}
impl BufferService {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let model = EditorModel::new(ropey::Rope::new());
        let published = Published {
            snapshot: model.buffer.create_snapshot(),
            selection: model.selection,
            document: 0,
            completed_through: 0,
            busy: None,
        };
        let shared = Arc::new(Shared {
            published: Mutex::new(Arc::new(published)),
            changed: Condvar::new(),
            admission: Mutex::new(Admission {
                next: 1,
                pending: 0,
                document: 0,
            }),
            stop: AtomicBool::new(false),
            wake: Arc::new(wake),
            progress: AtomicUsize::new(0),
            #[cfg(test)]
            gate: Mutex::new(None),
        });
        let (commands, input) = bounded(PENDING_LIMIT);
        let (output, completions) = bounded(PENDING_LIMIT);
        let (cancel, cancellation) = bounded(1);
        let (shutdown, exit) = bounded(1);
        let state = shared.clone();
        let thread = thread::Builder::new()
            .name("buffer-owner".into())
            .spawn(move || Actor::new(model, state, output).run(input, cancellation, exit))
            .expect("Could not start buffer owner");
        Self {
            client: BufferClient {
                commands,
                cancel,
                shared,
                completions,
            },
            shutdown,
            thread: Some(thread),
        }
    }
}
impl Drop for BufferService {
    fn drop(&mut self) {
        self.client.shared.stop.store(true, Ordering::Release);
        let _ = self.shutdown.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct Active {
    sequence: u64,
    context: EditContext,
    cancel: Arc<AtomicBool>,
}
struct WorkDone {
    sequence: u64,
    result: Result<(BufferSnapshot, SelectionState), String>,
}
struct Actor {
    model: EditorModel,
    document: u64,
    watermark: u64,
    done: BTreeSet<u64>,
    active: Option<Active>,
    shared: Arc<Shared>,
    output: Sender<Completion>,
    work: Sender<WorkDone>,
    results: Receiver<WorkDone>,
}
impl Actor {
    fn new(model: EditorModel, shared: Arc<Shared>, output: Sender<Completion>) -> Self {
        let (work, results) = bounded(1);
        Self {
            model,
            document: 0,
            watermark: 0,
            done: BTreeSet::new(),
            active: None,
            shared,
            output,
            work,
            results,
        }
    }
    fn context(&self) -> EditContext {
        EditContext {
            document: self.document,
            revision: self.model.buffer.create_snapshot().revision,
            cursor: self.model.buffer.cursor_offset(),
            selection: self.model.selection,
        }
    }
    fn publish(&self) {
        *self
            .shared
            .published
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Arc::new(Published {
            snapshot: self.model.buffer.create_snapshot(),
            selection: self.model.selection,
            document: self.document,
            completed_through: self.watermark,
            busy: self.active.as_ref().map(|a| a.sequence),
        });
        self.shared.changed.notify_all();
        (self.shared.wake)();
    }
    fn finish(&mut self, sequence: u64, result: Result<(), String>) {
        self.done.insert(sequence);
        while self.done.remove(&(self.watermark + 1)) {
            self.watermark += 1;
        }
        // Admission credits include undrained outcomes, so this bounded mailbox always has room.
        if self
            .output
            .try_send(Completion { sequence, result })
            .is_err()
        {
            self.shared.stop.store(true, Ordering::Release);
        }
        self.publish();
    }
    fn run(mut self, input: Receiver<Envelope>, cancel: Receiver<()>, exit: Receiver<()>) {
        loop {
            crossbeam_channel::select_biased! {
                recv(exit)->_=>break,
                recv(cancel)->_=>if let Some(active)=&self.active {active.cancel.store(true,Ordering::Release);},
                recv(self.results)->done=>if let Ok(done)=done {self.apply_done(done);},
                recv(input)->command=>match command {Ok(e)=>self.accept(e),Err(_)=>break},
            }
            if self.shared.stop.load(Ordering::Acquire) {
                break;
            }
        }
        if let Some(active) = self.active.take() {
            active.cancel.store(true, Ordering::Release);
        }
    }
    fn accept(&mut self, e: Envelope) {
        if let Command::Load(content) = e.command {
            if let Some(active) = &self.active {
                active.cancel.store(true, Ordering::Release);
            }
            self.document = e.document;
            self.model.buffer.load_document(content);
            self.model.selection = SelectionState::default();
            self.finish(e.sequence, Ok(()));
            return;
        }
        if e.document != self.document {
            self.finish(
                e.sequence,
                Err("Command belongs to an obsolete document".into()),
            );
            return;
        }
        match e.command {
            Command::Edit(command) if self.model.estimated_work(&command) >= BULK_THRESHOLD => {
                self.start_work(e.sequence, Command::Edit(command))
            }
            Command::Edit(command) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.model.apply(command)
                }))
                .unwrap_or_else(|_| Err("Editing command failed".into()));
                self.finish(e.sequence, result);
            }
            Command::BreakUndoGroup => {
                self.model.buffer.break_undo_group();
                self.finish(e.sequence, Ok(()));
            }
            command @ Command::Replace { .. } => self.start_work(e.sequence, command),
            Command::DeleteContext(context) => {
                if self.context() != context
                    || context.selection.mode == crate::selection::SelectionMode::None
                {
                    self.finish(
                        e.sequence,
                        Err("Cut skipped: document or selection changed".into()),
                    );
                } else {
                    self.start_work(
                        e.sequence,
                        Command::Edit(EditorCommand::Delete {
                            backward: false,
                            unit: crate::commands::DeleteUnit::Character,
                        }),
                    );
                }
            }
            Command::PasteContext { context, text } => {
                if self.context() != context {
                    self.finish(
                        e.sequence,
                        Err("Paste skipped: document or edit context changed".into()),
                    );
                } else {
                    let command = EditorCommand::Insert(text);
                    if self.model.estimated_work(&command) >= BULK_THRESHOLD {
                        self.start_work(e.sequence, Command::Edit(command));
                    } else {
                        let result = self.model.apply(command);
                        self.finish(e.sequence, result);
                    }
                }
            }

            Command::Load(_) => unreachable!(),
        }
    }
    fn start_work(&mut self, sequence: u64, command: Command) {
        if self.active.is_some() {
            self.finish(
                sequence,
                Err("Another bulk operation is already running".into()),
            );
            return;
        }
        let context = self.context();
        let snapshot = self.model.buffer.create_snapshot();
        let selection = self.model.selection;
        let cancel = Arc::new(AtomicBool::new(false));
        self.active = Some(Active {
            sequence,
            context,
            cancel: cancel.clone(),
        });
        self.shared.progress.store(0, Ordering::Relaxed);
        self.publish();
        let output = self.work.clone();
        let shared = self.shared.clone();
        thread::spawn(move || {
            let cancelled =
                || cancel.load(Ordering::Acquire) || shared.stop.load(Ordering::Acquire);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                {
                    let hook = shared.gate.lock().unwrap().take();
                    if let Some(hook) = hook {
                        hook();
                    }
                }
                if cancelled() {
                    return Err("Operation cancelled".into());
                }
                let mut edited = EditorModel::from_snapshot(&snapshot, selection);
                match command {
                    Command::Edit(command) => {
                        edited.apply_cancellable(command, &cancelled, &|done, _| {
                            shared.progress.store(done, Ordering::Relaxed);
                            (shared.wake)();
                        })?
                    }
                    Command::Replace {
                        query,
                        replacement,
                        all,
                    } => {
                        if let Some(plan) = texted_buffer::replacement::plan(
                            &snapshot,
                            &query,
                            &replacement,
                            all,
                            snapshot.cursor_offset,
                            cancelled,
                        )
                        .map_err(|e| format!("Replacement failed: {e:?}"))?
                        {
                            edited.buffer.load_document(plan.content);
                            edited.buffer.set_cursor_offset(plan.cursor);
                            edited.selection.mode = crate::selection::SelectionMode::None;
                        }
                    }
                    _ => return Err("Unsupported worker command".into()),
                }
                if cancelled() {
                    return Err("Operation cancelled".into());
                }
                Ok((edited.buffer.create_snapshot(), edited.selection))
            }))
            .unwrap_or_else(|_| Err("Bulk worker failed".into()));
            let _ = output.send(WorkDone { sequence, result });
        });
    }
    fn apply_done(&mut self, done: WorkDone) {
        let Some(active) = self.active.take() else {
            return;
        };
        if active.sequence != done.sequence {
            self.active = Some(active);
            return;
        }
        let result = if active.cancel.load(Ordering::Acquire) {
            Err("Operation cancelled".into())
        } else if active.context != self.context() {
            Err("Operation skipped: document, cursor or selection changed".into())
        } else {
            done.result.and_then(|(snapshot, selection)| {
                if snapshot
                    .content
                    .is_instance(&self.model.buffer.create_snapshot().content)
                {
                    self.model.buffer.set_cursor_offset(snapshot.cursor_offset);
                    self.model.selection = selection;
                    return Ok(());
                }
                if self.model.buffer.commit_snapshot(
                    active.context.revision,
                    active.context.cursor,
                    snapshot,
                ) {
                    self.model.selection = selection;
                    Ok(())
                } else {
                    Err("Operation skipped: document changed".into())
                }
            })
        };
        self.finish(done.sequence, result);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn submit(client: &BufferClient, command: Command) -> u64 {
        client
            .submit(command)
            .unwrap_or_else(|_| panic!("unexpected rejection"))
    }
    fn wait(client: &BufferClient, through: u64) -> Arc<Published> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut published = client.shared.published.lock().unwrap();
        while published.completed_through < through {
            let (next, timeout) = client
                .shared
                .changed
                .wait_timeout(
                    published,
                    deadline.saturating_duration_since(Instant::now()),
                )
                .unwrap();
            published = next;
            assert!(!timeout.timed_out());
        }
        published.clone()
    }
    #[test]
    fn a_ten_thousand_row_worker_commits_once_and_undo_restores_the_root() {
        let service = BufferService::new(|| {});
        let c = &service.client;
        submit(
            c,
            Command::Load(ropey::Rope::from_str(
                &format!("{}\n", "x".repeat(2048)).repeat(10000),
            )),
        );
        let original = wait(c, c.barrier());
        c.drain();
        submit(
            c,
            Command::Edit(EditorCommand::SetSelection(SelectionState {
                mode: crate::selection::SelectionMode::Block,
                anchor_line: 0,
                active_line: 9999,
                anchor_col: 2048,
                active_col: 2048,
            })),
        );
        submit(c, Command::Edit(EditorCommand::Insert("ї".into())));
        let edited = wait(c, c.barrier());
        assert_eq!(
            edited.snapshot.total_bytes,
            original.snapshot.total_bytes + 20000
        );
        assert!(c.drain().iter().all(|o| o.result.is_ok()));
        submit(c, Command::Edit(EditorCommand::Undo));
        assert!(wait(c, c.barrier())
            .snapshot
            .content
            .is_instance(&original.snapshot.content));
    }
    #[test]
    fn stale_cut_and_paste_contexts_do_not_edit_newer_content() {
        let service = BufferService::new(|| {});
        let c = &service.client;
        submit(c, Command::Load(ropey::Rope::from_str("old")));
        submit(c, Command::Edit(EditorCommand::SelectAll));
        let context = wait(c, c.barrier()).context();
        c.drain();
        submit(c, Command::Load(ropey::Rope::from_str("new")));
        submit(c, Command::DeleteContext(context));
        submit(
            c,
            Command::PasteContext {
                context,
                text: "late".into(),
            },
        );
        assert_eq!(wait(c, c.barrier()).snapshot.content.to_string(), "new");
        assert_eq!(c.drain().iter().filter(|o| o.result.is_err()).count(), 2);
    }
    #[test]
    fn accepted_commands_publish_snapshots_in_order_with_bounded_backpressure() {
        let service = BufferService::new(|| {});
        let c = &service.client;
        for _ in 0..PENDING_LIMIT {
            submit(c, Command::Edit(EditorCommand::Insert("ї".into())));
        }
        assert!(matches!(
            c.submit(Command::Edit(EditorCommand::Insert("x".into()))),
            Err(SubmitError::Busy(_))
        ));
        let p = wait(c, c.barrier());
        assert_eq!(p.snapshot.content.to_string(), "ї".repeat(PENDING_LIMIT));
        assert_eq!(c.drain().len(), PENDING_LIMIT);
        submit(c, Command::Edit(EditorCommand::Undo));
        assert_eq!(wait(c, c.barrier()).snapshot.total_bytes, 0);
    }
    #[test]
    fn a_slow_bulk_worker_does_not_block_typing_or_completion_barriers() {
        let service = BufferService::new(|| {});
        let c = &service.client;
        let (started, ready) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        *c.shared.gate.lock().unwrap() = Some(Box::new(move || {
            started.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(30)).unwrap();
        }));
        let first = submit(
            c,
            Command::Edit(EditorCommand::Insert("a".repeat(BULK_THRESHOLD))),
        );
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        submit(c, Command::Edit(EditorCommand::Insert("x".into())));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut p = c.shared.published.lock().unwrap();
        while p.snapshot.total_bytes == 0 {
            let (next, timeout) = c
                .shared
                .changed
                .wait_timeout(p, deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            p = next;
            assert!(!timeout.timed_out());
        }
        assert!(p.completed_through < first);
        assert_eq!(p.snapshot.content.to_string(), "x");
        drop(p);
        c.cancel();
        release.send(()).unwrap();
        assert_eq!(wait(c, c.barrier()).snapshot.content.to_string(), "x");
        assert!(c
            .drain()
            .iter()
            .any(|o| o.sequence == first && o.result.is_err()));
    }
    #[test]
    fn document_switch_rejects_a_stale_bulk_result() {
        let service = BufferService::new(|| {});
        let c = &service.client;
        let (started, ready) = std::sync::mpsc::channel();
        let (release, gate) = std::sync::mpsc::channel();
        *c.shared.gate.lock().unwrap() = Some(Box::new(move || {
            started.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(30)).unwrap();
        }));
        submit(
            c,
            Command::Edit(EditorCommand::Insert("a".repeat(BULK_THRESHOLD))),
        );
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        submit(c, Command::Load(ropey::Rope::from_str("new")));
        release.send(()).unwrap();
        let p = wait(c, c.barrier());
        assert_eq!(p.document, 1);
        assert_eq!(p.snapshot.content.to_string(), "new");
    }
}
