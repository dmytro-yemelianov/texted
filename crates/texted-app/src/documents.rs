//! Single-document file lifecycle. Native sheets are polled on main; file I/O runs on snapshots.
use ropey::Rope;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{mpsc, Arc},
    task::{Context, Poll, Wake, Waker},
    thread,
};
use texted_app::buffer_service::{BufferClient, Command};
use texted_buffer::BufferSnapshot;
use texted_io::{IoManager, LineEnding, LoadedDocument};
use winit::window::Window;
/// A user-requested file operation must not wait on a hidden or minimized sheet.
pub(crate) fn reveal_for_dialog(window: &Window) {
    window.set_visible(true);
    window.set_minimized(false);
    window.focus_window();
}
#[derive(Debug, Clone)]
pub enum Action {
    OpenDialog,
    OpenPath(PathBuf),
    New,
    Save { save_as: bool },
    ReportError(String),
    Close,
}
pub enum Effect {
    None,
    Install(LoadedDocument),
    Close,
}
type NativeFuture<T> = Pin<Box<dyn Future<Output = T>>>;
struct Saved {
    path: PathBuf,
    text: Rope,
}
enum Job {
    Save(mpsc::Receiver<Result<Saved, String>>),
    Load(mpsc::Receiver<Result<LoadedDocument, String>>),
    ChooseSave {
        future: NativeFuture<Option<rfd::FileHandle>>,
        snapshot: BufferSnapshot,
    },
    ChooseOpen(NativeFuture<Option<rfd::FileHandle>>),
    Confirm {
        future: NativeFuture<rfd::MessageDialogResult>,
        action: Action,
    },
    Error(NativeFuture<rfd::MessageDialogResult>),
}
struct DialogWake(crate::Wake);
impl Wake for DialogWake {
    fn wake(self: Arc<Self>) {
        (self.0)();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        (self.0)();
    }
}
fn poll_native<T>(future: &mut NativeFuture<T>, wake: &crate::Wake) -> Poll<T> {
    let waker = Waker::from(Arc::new(DialogWake(wake.clone())));
    future.as_mut().poll(&mut Context::from_waker(&waker))
}
pub struct Documents {
    pub path: Option<PathBuf>,
    pub bom: bool,
    pub ending: LineEnding,
    saved_text: Rope,
    wake: crate::Wake,
    job: Option<Job>,
    next: Option<Action>,
}
impl Documents {
    pub fn new(
        text: Rope,
        path: Option<PathBuf>,
        bom: bool,
        ending: LineEnding,
        wake: crate::Wake,
    ) -> Self {
        Self {
            path,
            bom,
            ending,
            saved_text: text,
            wake,
            job: None,
            next: None,
        }
    }
    pub fn dirty(&self, snapshot: &BufferSnapshot) -> bool {
        !snapshot.content.is_instance(&self.saved_text)
    }
    /// Accept external lifecycle actions only after a dialog, worker and its continuation finish.
    pub fn is_idle(&self) -> bool {
        self.job.is_none() && self.next.is_none()
    }
    pub fn locked(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|j| !matches!(j, Job::Save(_)))
            || self.next.is_some()
    }
    pub fn title(&self, snapshot: &BufferSnapshot) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!(
            "{name}{}{} - Texted",
            if self.dirty(snapshot) { " *" } else { "" },
            match self.job {
                Some(Job::Load(_)) => " (Opening…)",
                Some(Job::Save(_)) => " (Saving…)",
                _ => "",
            }
        )
    }
    pub fn installed(&mut self, document: &LoadedDocument) {
        self.path = document.path.clone();
        self.bom = document.has_bom;
        self.ending = document.line_ending;
        self.saved_text = document.content.clone();
    }
    fn error(&mut self, window: &Arc<Window>, message: &str) {
        reveal_for_dialog(window);
        eprintln!("{message}");
        self.next = None;
        self.job = Some(Job::Error(Box::pin(
            rfd::AsyncMessageDialog::new()
                .set_parent(window.as_ref())
                .set_title("Texted")
                .set_description(message)
                .set_level(rfd::MessageLevel::Error)
                .show(),
        )));
        window.request_redraw();
    }
    pub fn handle(
        &mut self,
        action: Action,
        snapshot: &BufferSnapshot,
        client: &BufferClient,
        window: &Arc<Window>,
    ) -> Effect {
        if self.job.is_some() {
            if self.next.is_none() {
                self.next = Some(action);
            }
            return Effect::None;
        }
        if let Action::Save { save_as } = action {
            self.save(snapshot, save_as, client, window);
            return Effect::None;
        }
        if let Action::ReportError(message) = action {
            self.error(window, &message);
            return Effect::None;
        }
        if self.dirty(snapshot) {
            reveal_for_dialog(window);
            let future = Box::pin(
                rfd::AsyncMessageDialog::new()
                    .set_parent(window.as_ref())
                    .set_title("Save changes?")
                    .set_description("Your changes will be lost if you don't save them.")
                    .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                        "Save".into(),
                        "Discard".into(),
                        "Cancel".into(),
                    ))
                    .show(),
            );
            self.job = Some(Job::Confirm { future, action });
            window.request_redraw();
            return Effect::None;
        }
        self.proceed(action, window)
    }
    fn proceed(&mut self, action: Action, window: &Arc<Window>) -> Effect {
        match action {
            Action::Close => Effect::Close,
            Action::New => Effect::Install(LoadedDocument {
                content: Rope::new(),
                path: None,
                has_bom: false,
                line_ending: LineEnding::Lf,
                byte_size: 0,
            }),
            Action::OpenDialog => {
                reveal_for_dialog(window);
                let mut dialog = rfd::AsyncFileDialog::new()
                    .set_parent(window.as_ref())
                    .set_title("Open plain-text file");
                if let Some(parent) = self.path.as_ref().and_then(|p| p.parent()) {
                    dialog = dialog.set_directory(parent);
                }
                self.job = Some(Job::ChooseOpen(Box::pin(dialog.pick_file())));
                window.request_redraw();
                Effect::None
            }
            Action::OpenPath(path) => {
                self.open(path, window);
                Effect::None
            }
            Action::Save { .. } | Action::ReportError(_) => unreachable!(),
        }
    }
    fn open(&mut self, path: PathBuf, window: &Arc<Window>) {
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job::Load(rx));
        window.request_redraw();
        let wake = self.wake.clone();
        thread::spawn(move || {
            let result = IoManager::open_file(&path)
                .map_err(|e| format!("Could not open {}: {e}", path.display()));
            let _ = tx.send(result);
            wake();
        });
    }
    fn save(
        &mut self,
        snapshot: &BufferSnapshot,
        save_as: bool,
        client: &BufferClient,
        window: &Arc<Window>,
    ) {
        if client.submit(Command::BreakUndoGroup).is_err() {
            self.error(
                window,
                "Buffer is busy; save was not accepted. Please retry.",
            );
            return;
        }
        if save_as || self.path.is_none() {
            reveal_for_dialog(window);
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_parent(window.as_ref())
                .set_title("Save plain-text file")
                .set_file_name(
                    self.path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Untitled.txt".into()),
                );
            if let Some(parent) = self.path.as_ref().and_then(|p| p.parent()) {
                dialog = dialog.set_directory(parent);
            }
            self.job = Some(Job::ChooseSave {
                future: Box::pin(dialog.save_file()),
                snapshot: snapshot.clone(),
            });
            window.request_redraw();
        } else {
            self.write(snapshot, self.path.clone().unwrap(), window);
        }
    }
    fn write(&mut self, snapshot: &BufferSnapshot, path: PathBuf, window: &Arc<Window>) {
        let (tx, rx) = mpsc::channel();
        self.job = Some(Job::Save(rx));
        window.request_redraw();
        let content = snapshot.content.clone();
        let bom = self.bom;
        let ending = self.ending;
        let wake = self.wake.clone();
        thread::spawn(move || {
            let result = IoManager::save_rope(&path, &content, bom, ending)
                .map_err(|e| format!("Could not save {}: {e}", path.display()))
                .map(|()| Saved {
                    path,
                    text: content,
                });
            let _ = tx.send(result);
            wake();
        });
    }
    pub fn poll(
        &mut self,
        snapshot: &BufferSnapshot,
        client: &BufferClient,
        window: &Arc<Window>,
    ) -> Effect {
        if let Some(mut job) = self.job.take() {
            match &mut job {
                Job::Save(rx) => match rx.try_recv() {
                    Ok(Ok(saved)) => {
                        self.path = Some(saved.path);
                        self.saved_text = saved.text;
                    }
                    Ok(Err(error)) => {
                        self.error(window, &error);
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.error(window, "The save worker stopped before completing.");
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::Load(rx) => match rx.try_recv() {
                    Ok(Ok(document)) => return Effect::Install(document),
                    Ok(Err(error)) => {
                        self.error(window, &error);
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.error(window, "The file loader stopped before completing.");
                        return Effect::None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::ChooseSave {
                    future,
                    snapshot: written,
                } => match poll_native(future, &self.wake) {
                    Poll::Ready(Some(file)) => {
                        self.write(written, file.path().into(), window);
                        return Effect::None;
                    }
                    Poll::Ready(None) => {
                        self.next = None;
                    }
                    Poll::Pending => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::ChooseOpen(future) => match poll_native(future, &self.wake) {
                    Poll::Ready(Some(file)) => {
                        self.open(file.path().into(), window);
                        return Effect::None;
                    }
                    Poll::Ready(None) => {}
                    Poll::Pending => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::Confirm { future, action } => match poll_native(future, &self.wake) {
                    Poll::Ready(choice) => {
                        if choice == rfd::MessageDialogResult::Yes
                            || choice == rfd::MessageDialogResult::Custom("Save".into())
                        {
                            self.next = Some(action.clone());
                            self.save(snapshot, false, client, window);
                            return Effect::None;
                        }
                        if choice == rfd::MessageDialogResult::No
                            || choice == rfd::MessageDialogResult::Custom("Discard".into())
                        {
                            return self.proceed(action.clone(), window);
                        }
                        self.next = None;
                    }
                    Poll::Pending => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
                Job::Error(future) => match poll_native(future, &self.wake) {
                    Poll::Ready(_) => {}
                    Poll::Pending => {
                        self.job = Some(job);
                        return Effect::None;
                    }
                },
            }
        }
        if let Some(action) = self.next.take() {
            return self.handle(action, snapshot, client, window);
        }
        Effect::None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use texted_buffer::TextBuffer;
    #[test]
    fn native_dialog_completion_wakes_an_undispatched_ui_queue() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (send, received) = mpsc::channel();
        let wake: crate::Wake = Arc::new(move || {
            let _ = send.send(());
        });
        let done = Arc::new(AtomicBool::new(false));
        let state = done.clone();
        let mut future: NativeFuture<()> = Box::pin(std::future::poll_fn(move |cx| {
            if state.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            let state = state.clone();
            let waker = cx.waker().clone();
            thread::spawn(move || {
                state.store(true, Ordering::Release);
                waker.wake();
            });
            Poll::Pending
        }));
        assert!(poll_native(&mut future, &wake).is_pending());
        received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(poll_native(&mut future, &wake).is_ready());
    }
    #[test]
    fn actions_wait_for_file_work_and_its_continuation() {
        let mut doc = Documents::new(Rope::new(), None, false, LineEnding::Lf, Arc::new(|| {}));
        assert!(doc.is_idle());
        let (_tx, rx) = mpsc::channel();
        doc.job = Some(Job::Save(rx));
        assert!(!doc.is_idle());
        doc.next = Some(Action::Close);
        doc.job = None;
        assert!(!doc.is_idle());
        doc.next = None;
        assert!(doc.is_idle());
    }
    #[test]
    fn undo_to_saved_root_is_clean_and_cursor_changes_are_not_edits() {
        let mut b = TextBuffer::from_string("original".into());
        let doc = Documents::new(
            b.create_snapshot().content,
            None,
            false,
            LineEnding::Lf,
            Arc::new(|| {}),
        );
        b.set_cursor_offset(3);
        assert!(!doc.dirty(&b.create_snapshot()));
        b.insert(3, "ї");
        assert!(doc.dirty(&b.create_snapshot()));
        assert!(b.undo());
        assert!(!doc.dirty(&b.create_snapshot()));
    }
    #[test]
    fn saving_old_snapshot_does_not_clean_newer_edits() {
        let mut b = TextBuffer::new();
        b.insert(0, "first");
        let written = b.create_snapshot().content;
        b.insert(5, "second");
        let doc = Documents::new(
            written,
            Some("file.txt".into()),
            false,
            LineEnding::Lf,
            Arc::new(|| {}),
        );
        assert!(doc.dirty(&b.create_snapshot()));
    }
}
